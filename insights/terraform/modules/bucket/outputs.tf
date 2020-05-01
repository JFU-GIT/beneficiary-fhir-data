output "arn" {
  value = aws_s3_bucket.main.arn
}

output "id" {
  value = aws_s3_bucket.main.id
}

output "bucket_cmk" {
  value = aws_kms_key.main.key_id
}